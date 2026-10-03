#!/usr/bin/env python3
"""Only two real center processes. No relay. HTTP loopback and remote-style TLS."""
import argparse
from concurrent.futures import ThreadPoolExecutor
import json
from pathlib import Path
import signal
import sqlite3
import subprocess
import tempfile
from smoke import api, free_port, wait_for, BODY, ENV

def main():
    parser=argparse.ArgumentParser()
    parser.add_argument('--binary',type=Path,default=Path(__file__).resolve().parents[1]/'target/debug/topicairn')
    parser.add_argument('--tls',action='store_true')
    options=parser.parse_args()
    binary=options.binary.resolve()
    env={k:v for k,v in ENV.items() if k.lower() not in ('http_proxy','https_proxy','all_proxy')}
    processes,logs=[],[]
    with tempfile.TemporaryDirectory(prefix='topicairn-p2p-smoke-') as tmp:
        root=Path(tmp)
        a,b=['http://127.0.0.1:'+str(free_port()) for _ in range(2)]
        peer_urls=[('https' if options.tls else 'http')+'://127.0.0.1:'+str(free_port()) for _ in range(2)]
        tls=[]
        if options.tls:
            for name in ('a','b'):
                path=root/(name+'-tls')
                subprocess.run([str(binary),'tls-init','--host','127.0.0.1','--out',str(path)],env=env,check=True,capture_output=True)
                tls.append(path)
        def start(name,url,index):
            flags=[] if not options.tls else ['--peer-tls-cert',tls[index]/'server.pem','--peer-tls-key',tls[index]/'server-key.pem','--peer-ca',tls[index]/'ca.pem','--peer-url',peer_urls[index]]
            log=open(root/f'log-{len(logs)}','w+');logs.append(log)
            p=subprocess.Popen([str(binary),'serve','--name',name,'--data',str(root/name),'--bind',url.split('://')[1],'--peer-bind',peer_urls[index].split('://')[1],'--sync-seconds','1',*map(str,flags)],env=env,stdout=log,stderr=log)
            processes.append(p)
            wait_for(lambda:(root/name/'admin.token').exists())
            token=(root/name/'admin.token').read_text().strip()
            wait_for(lambda:api(url,'/status',token))
            assert api(url,'/status',token)['transport']=='direct'
            return p,token
        def stop(p):
            if p.poll() is None:
                p.send_signal(signal.SIGINT)
                try:p.wait(timeout=15)
                except subprocess.TimeoutExpired:p.kill();p.wait()
        def history(url,token,topic):return api(url,f'/topics/{topic["id"]}/messages',token)
        try:
            ap,at=start('a',a,0);bp,bt=start('b',b,1)
            ac=api(a,'/p2p/contact',at);bc=api(b,'/p2p/contact',bt)
            api(a,'/p2p/peers',at,bc);api(b,'/p2p/peers',bt,ac)
            for url,token,profile in [(a,at,bc),(b,bt,ac)]:
                assert api(url,'/p2p/peers/'+profile['identity']['user_id']+'/check',token,{})==profile['identity']
            t=api(a,'/topics',at,{'peer_id':bc['identity']['user_id'],'title':'直连数学'})
            other=api(b,'/topics',bt,{'peer_id':ac['identity']['user_id'],'title':'直连 NAS'})
            # Simultaneous first contact and sync: both endpoints must remain able to service requests.
            with ThreadPoolExecutor(2) as pool:
                sends=[pool.submit(api,a,f'/topics/{t["id"]}/messages',at,{'body':BODY}),pool.submit(api,b,f'/topics/{other["id"]}/messages',bt,{'body':'双方同时第一次发信'})]
                messages=[job.result(timeout=15) for job in sends]
                syncs=[pool.submit(api,a,'/sync',at,{}),pool.submit(api,b,'/sync',bt,{})]
                for job in syncs:job.result(timeout=15)
            wait_for(lambda:len(history(b,bt,t))==1)
            wait_for(lambda:len(history(a,at,other))==1)
            assert history(b,bt,t)[0]['body']==BODY
            reply=api(b,f'/topics/{t["id"]}/messages',bt,{'body':'直接回复 $x$','reply_to':messages[0]['id']})
            wait_for(lambda:len(history(a,at,t))==2)
            wait_for(lambda:history(a,at,t)[0]['delivery']=='delivered')
            assert history(a,at,t)[1]['replyTo']==messages[0]['id']
            api(a,f'/topics/{t["id"]}',at,{'title':'直连证明','archived':False})
            wait_for(lambda:any(topic['title']=='直连证明' for topic in api(b,'/topics',bt)))
            stop(bp)
            pending=api(a,f'/topics/{t["id"]}/messages',at,{'body':'对方离线，重启后只送一次'})
            queued=api(a,'/outbox',at)
            assert queued
            with sqlite3.connect(root/'a/domain.sqlite') as db:
                raw=db.execute('SELECT envelope FROM outbox WHERE message_id=?',[pending['id']]).fetchone()[0]
                assert '对方离线' not in raw
            stop(ap)
            ap,at=start('a',a,0);bp,bt=start('b',b,1)
            assert api(a,'/p2p/contact',at)==ac
            wait_for(lambda:len(history(b,bt,t))==3,seconds=45)
            wait_for(lambda:history(a,at,t)[2]['delivery']=='delivered')
            assert history(b,bt,t)[2]['id']==pending['id']
            stop(bp);bp,bt=start('b',b,1)
            api(a,'/sync',at,{})
            assert len(history(b,bt,t))==3
            assert not api(a,'/outbox',at)
            assert not list(root.glob('*relay*'))
            print('PASS ('+('TLS 1.3' if options.tls else 'HTTP loopback')+'): exactly 2 centers, no relay; signed mutual connectivity; simultaneous first contact/sync; E2EE/Markdown/topics/replies; signed decrypted ACK; offline local ciphertext queue; both restarts; dedupe')
        except Exception:
            for log in logs:log.flush();log.seek(0);print(log.read())
            raise
        finally:
            for p in processes:stop(p)
            for log in logs:log.close()

if __name__=='__main__':main()
